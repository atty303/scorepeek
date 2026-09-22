use std::collections::BTreeSet;
use std::error::Error;
use std::fmt;

use serde::Serialize;
use unicode_normalization::UnicodeNormalization as _;

use crate::catalog::{Catalog, DisplayVariantKind, ScorepeekSongId};

pub const DIAGNOSTIC_TITLE_COMPARISON_KEY_ID: &str =
    "scorepeek-title-nfc-ucd17-exact-then-ascii-width-fold-v2";
pub const DIAGNOSTIC_TITLE_MINIMUM_CONFIDENCE: f64 = 0.95;

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
    let observed_exact_key = exact_comparison_key(ocr_text);
    let observed_folded_key = folded_comparison_key(ocr_text);
    let mut exact_matches = BTreeSet::new();
    let mut folded_matches = BTreeSet::new();
    for (id, kind, value) in candidates {
        if kind == DisplayVariantKind::SearchTerm {
            continue;
        }
        if exact_comparison_key(value) == observed_exact_key {
            exact_matches.insert(id);
        }
        if folded_comparison_key(value) == observed_folded_key {
            folded_matches.insert(id);
        }
    }
    candidate_match(if exact_matches.is_empty() {
        folded_matches
    } else {
        exact_matches
    })
}

fn candidate_match<T: Copy + Ord>(matches: BTreeSet<T>) -> CandidateMatch<T> {
    let mut matches = matches.into_iter();
    match (matches.next(), matches.next()) {
        (None, _) => CandidateMatch::None,
        (Some(id), None) => CandidateMatch::Unique(id),
        (Some(_), Some(_)) => CandidateMatch::Ambiguous,
    }
}

pub(in crate::recognition) fn exact_comparison_key(value: &str) -> String {
    value.nfc().filter(|character| *character != ' ').collect()
}

pub(in crate::recognition) fn folded_comparison_key(value: &str) -> String {
    value
        .nfc()
        .filter_map(|character| match character {
            ' ' | '\u{3000}' => None,
            '\u{ff01}'..='\u{ff5e}' => char::from_u32(u32::from(character) - 0xfee0),
            _ => Some(character),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{
        CandidateMatch, DiagnosticTitleCandidate, DiagnosticTitleError,
        DiagnosticTitleUnknownReason, diagnostic_title_candidate, exact_comparison_key,
        folded_comparison_key, unique_candidate,
    };
    use crate::catalog::{Catalog, DisplayVariantKind};

    #[test]
    fn comparison_keys_preserve_exact_tier_and_bound_ascii_width_fallback() {
        assert_eq!(exact_comparison_key("ABSOLUTE EVIL"), "ABSOLUTEEVIL");
        assert_eq!(
            exact_comparison_key("ＰＡＳＴＥＬＩＳＭ"),
            "ＰＡＳＴＥＬＩＳＭ"
        );
        assert_eq!(folded_comparison_key("Cafe\u{301} Noir"), "Caf\u{e9}Noir");
        assert_eq!(folded_comparison_key("ＰＡＳＴＥＬＩＳＭ"), "PASTELISM");
        assert_eq!(folded_comparison_key("Ａ！　Ｂ～"), "A!B~");
        assert_eq!(folded_comparison_key("Absolute\tEvil"), "Absolute\tEvil");
        assert_eq!(
            folded_comparison_key("Absolute\u{a0}Evil"),
            "Absolute\u{a0}Evil"
        );
        assert_eq!(folded_comparison_key("Ⅰ①ｶ"), "Ⅰ①ｶ");
        assert_eq!(
            folded_comparison_key("a\u{0897}\u{0316}"),
            folded_comparison_key("a\u{0316}\u{0897}")
        );
        let fullwidth_ascii: String = (0xff01..=0xff5e).filter_map(char::from_u32).collect();
        let ascii: String = (0x21..=0x7e).filter_map(char::from_u32).collect();
        assert_eq!(folded_comparison_key(&fullwidth_ascii), ascii);
        assert_ne!(
            folded_comparison_key("ABSOLUTE EVIL"),
            folded_comparison_key("Absolute Evil")
        );
        assert_ne!(folded_comparison_key("A-B"), folded_comparison_key("AB"));
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
    fn candidate_match_accepts_ascii_ocr_for_fullwidth_catalog_title() {
        assert_eq!(
            unique_candidate(
                "PASTELISM",
                [
                    (1, DisplayVariantKind::InGameDisplay, "ＰＡＳＴＥＬＩＳＭ"),
                    (1, DisplayVariantKind::OfficialDisplay, "ＰＡＳＴＥＬＩＳＭ"),
                ],
            ),
            CandidateMatch::Unique(1)
        );
    }

    #[test]
    fn candidate_match_prefers_exact_tier_over_folded_collision() {
        assert_eq!(
            unique_candidate(
                "A!",
                [
                    (1, DisplayVariantKind::InGameDisplay, "A!"),
                    (2, DisplayVariantKind::InGameDisplay, "Ａ！"),
                ],
            ),
            CandidateMatch::Unique(1)
        );
    }

    #[test]
    fn unicode_17_combining_order_matches_the_python_contract() {
        assert_eq!(
            unique_candidate(
                "a\u{0897}\u{0316}",
                [
                    (1, DisplayVariantKind::InGameDisplay, "a\u{0897}\u{0316}"),
                    (2, DisplayVariantKind::InGameDisplay, "a\u{0316}\u{0897}"),
                ],
            ),
            CandidateMatch::Ambiguous
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
