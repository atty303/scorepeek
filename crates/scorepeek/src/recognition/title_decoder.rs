use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::io::Read as _;
use std::path::Path;

use serde::Serialize;

use crate::catalog::{Catalog, DisplayVariantKind, ScorepeekSongId};
use crate::recognition::ctc_sequence::CtcSequenceTrie;
use crate::recognition::title::ctc_candidate_sequences;

pub const TITLE_DICTIONARY_SHA256: &str =
    "ab078671bb49f06228eadccd34f1bb501e157f7a047095ffb943ba81512c77d1";
const MAX_INFERENCE_YML_BYTES: u64 = 2 * 1024 * 1024;
const OUTPUT_CLASSES: usize = 18_710;
const OUTPUT_TIMESTEPS: usize = 40;
const MAX_TITLE_TOKENS: usize = OUTPUT_TIMESTEPS;

#[derive(Debug)]
pub enum CatalogTitleDecoderError {
    Io(std::io::Error),
    InvalidDictionary,
    InvalidCatalogTitle,
    InvalidProbabilities,
    InvalidThresholds,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct CatalogTitleDictionaryAudit {
    pub schema: &'static str,
    pub dictionary_sha256: &'static str,
    pub maximum_ctc_timesteps: usize,
    pub song_count: usize,
    pub non_search_variant_count: usize,
    pub encodable_variant_count: usize,
    pub rejected_variant_count: usize,
    pub songs_without_non_search_variant: usize,
    pub coverage_complete: bool,
    pub by_variant_kind: Vec<TitleDictionaryVariantKindAudit>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct TitleDictionaryVariantKindAudit {
    pub kind: DisplayVariantKind,
    pub variant_count: usize,
    pub encodable_variant_count: usize,
    pub rejected_variant_count: usize,
    pub unsupported_character_variant_count: usize,
    pub ctc_timestep_excess_variant_count: usize,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct TitleModelExportRequirements {
    pub schema: &'static str,
    pub baseline_dictionary_sha256: &'static str,
    pub dictionary_contract_id: &'static str,
    pub output_tensor_contract_id: &'static str,
    pub ctc_blank_token: u32,
    pub output_timesteps: usize,
    pub output_classes: usize,
    pub baseline_character_count: usize,
    pub appended_catalog_character_count: usize,
    pub non_search_variant_count: usize,
    pub covered_variant_count: usize,
    pub coverage_complete: bool,
    pub non_blank_tokens: Vec<String>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct VariantCoverageCounts {
    variants: usize,
    encodable: usize,
    unsupported_characters: usize,
    ctc_timestep_excess: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct TitleEncodability {
    unsupported_characters: bool,
    ctc_timestep_excess: bool,
}

impl std::fmt::Display for CatalogTitleDecoderError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "catalog title decoder I/O failed: {error}"),
            Self::InvalidDictionary => {
                formatter.write_str("catalog title decoder dictionary is invalid")
            }
            Self::InvalidCatalogTitle => {
                formatter.write_str("catalog contains a title outside the model export contract")
            }
            Self::InvalidProbabilities => {
                formatter.write_str("catalog title probability tensor is invalid")
            }
            Self::InvalidThresholds => formatter.write_str("catalog title thresholds are invalid"),
        }
    }
}

impl std::error::Error for CatalogTitleDecoderError {}

impl From<std::io::Error> for CatalogTitleDecoderError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

#[derive(Clone, Copy, Debug, Serialize)]
pub struct DiagnosticTitleThresholds {
    pub minimum_log_probability: f64,
    pub minimum_runner_up_margin: f64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CatalogTitleUnknownReason {
    NoEncodableCandidate,
    CatalogCoverageIncomplete,
    AmbiguousTopCandidate,
    InsufficientAbsoluteEvidence,
    InsufficientRunnerUpMargin,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum CatalogTitleDecision {
    Unique {
        song_id: ScorepeekSongId,
        log_probability: f64,
        runner_up_margin: f64,
    },
    Unknown {
        reason: CatalogTitleUnknownReason,
    },
}

/// Scores exact catalog titles and song-unique comparison-key aliases through one CTC prefix trie.
///
/// This is an offline diagnostic boundary. Thresholds are explicit because the current profile
/// has no calibrated acceptance policy. A title that cannot be represented by the registered
/// model dictionary is not approximated. Bounded aliases use the registered title comparison key;
/// a folded key shared by multiple songs is never added as a decision sequence.
///
/// # Errors
/// Returns an error for an unregistered dictionary, malformed probability tensor, or invalid
/// thresholds.
pub fn score_catalog_titles(
    probabilities: &[f32],
    catalog: &Catalog,
    inference_yml: impl AsRef<Path>,
    thresholds: DiagnosticTitleThresholds,
) -> Result<CatalogTitleDecision, CatalogTitleDecoderError> {
    validate_probabilities(probabilities)?;
    if !thresholds.minimum_log_probability.is_finite()
        || !thresholds.minimum_runner_up_margin.is_finite()
        || thresholds.minimum_runner_up_margin < 0.0
    {
        return Err(CatalogTitleDecoderError::InvalidThresholds);
    }
    let dictionary = load_dictionary(inference_yml.as_ref())?;
    let indexes = dictionary_indexes(&dictionary)?;
    let mut trie = CtcSequenceTrie::default();
    let mut catalog_coverage_complete = true;
    for song in catalog.songs().values() {
        let mut has_non_search_variant = false;
        for variant in song
            .title_variants()
            .iter()
            .filter(|variant| variant.kind != DisplayVariantKind::SearchTerm)
        {
            has_non_search_variant = true;
            if tokenize(&variant.value, &indexes).is_none() {
                catalog_coverage_complete = false;
            }
        }
        catalog_coverage_complete &= has_non_search_variant;
    }
    let candidates = catalog.songs().iter().flat_map(|(song_id, song)| {
        song.title_variants()
            .iter()
            .map(move |variant| (*song_id, variant.kind, variant.value.as_str()))
    });
    for (song_id, sequences) in ctc_candidate_sequences(candidates) {
        for sequence in sequences {
            let Some(tokens) = tokenize(&sequence, &indexes) else {
                continue;
            };
            if !trie.insert(&tokens, song_id) {
                return Err(CatalogTitleDecoderError::InvalidDictionary);
            }
        }
    }
    if trie.is_empty() {
        return Ok(CatalogTitleDecision::Unknown {
            reason: CatalogTitleUnknownReason::NoEncodableCandidate,
        });
    }

    let scores = trie
        .score(probabilities, OUTPUT_CLASSES)
        .ok_or(CatalogTitleDecoderError::InvalidProbabilities)?;
    let mut songs = BTreeMap::<ScorepeekSongId, f64>::new();
    for (song_id, score) in scores.values {
        songs
            .entry(*song_id)
            .and_modify(|existing| *existing = existing.max(score))
            .or_insert(score);
    }
    let mut ranked: Vec<_> = songs.into_iter().collect();
    ranked.sort_by(|left, right| {
        right
            .1
            .total_cmp(&left.1)
            .then_with(|| left.0.cmp(&right.0))
    });
    Ok(decide_ranked(
        &ranked,
        thresholds,
        catalog_coverage_complete,
    ))
}

/// Audits exact catalog-title coverage of the immutable registered OCR dictionary.
///
/// The report contains aggregate counts only. It does not expose catalog strings, silently omit
/// rejected variants, or treat search aliases as display-title candidates.
///
/// # Errors
/// Returns an error when the supplied file is not the registered dictionary.
pub fn audit_catalog_title_dictionary(
    catalog: &Catalog,
    inference_yml: impl AsRef<Path>,
) -> Result<CatalogTitleDictionaryAudit, CatalogTitleDecoderError> {
    let dictionary = load_dictionary(inference_yml.as_ref())?;
    let indexes = dictionary_indexes(&dictionary)?;
    Ok(audit_catalog_title_dictionary_with_indexes(
        catalog, &indexes,
    ))
}

/// Defines the complete dictionary and tensor shape required by a scorepeek-owned title model.
///
/// The new scalar dictionary retains the character coverage of the registered baseline and appends
/// every character used by every non-search catalog variant. The timestep count is raised to the
/// largest exact CTC alignment required by that same complete variant set. Nothing is omitted for
/// being unsupported by the baseline model.
///
/// # Errors
/// Returns an error for an unregistered baseline dictionary, an empty variant set, or a catalog
/// title that is empty, contains control characters, or exceeds the bounded export shape.
pub fn title_model_export_requirements(
    catalog: &Catalog,
    inference_yml: impl AsRef<Path>,
) -> Result<TitleModelExportRequirements, CatalogTitleDecoderError> {
    let dictionary = load_dictionary(inference_yml.as_ref())?;
    let variants = catalog.songs().values().flat_map(|song| {
        song.title_variants()
            .iter()
            .filter(|variant| variant.kind != DisplayVariantKind::SearchTerm)
            .map(|variant| variant.value.as_str())
    });
    build_title_model_export_requirements(&dictionary, variants)
}

fn build_title_model_export_requirements<'a>(
    baseline_dictionary: &[String],
    variants: impl Iterator<Item = &'a str>,
) -> Result<TitleModelExportRequirements, CatalogTitleDecoderError> {
    let mut characters = Vec::new();
    let mut retained = BTreeSet::new();
    for entry in baseline_dictionary.iter().skip(1) {
        for character in entry.chars().filter(|character| !character.is_control()) {
            if retained.insert(character) {
                characters.push(character);
            }
        }
    }
    let baseline_character_count = characters.len();
    if !retained.contains(&' ') {
        return Err(CatalogTitleDecoderError::InvalidCatalogTitle);
    }
    characters.retain(|character| *character != ' ');
    let mut catalog_characters = BTreeSet::new();
    let mut non_search_variant_count = 0_usize;
    let mut required_timesteps = 0_usize;
    for title in variants {
        if title.is_empty() || title.chars().any(char::is_control) {
            return Err(CatalogTitleDecoderError::InvalidCatalogTitle);
        }
        non_search_variant_count += 1;
        let title_characters: Vec<_> = title.chars().collect();
        let timesteps = title_characters.len()
            + title_characters
                .windows(2)
                .filter(|pair| pair[0] == pair[1])
                .count();
        required_timesteps = required_timesteps.max(timesteps);
        catalog_characters.extend(title_characters);
    }
    if non_search_variant_count == 0 || required_timesteps > 512 {
        return Err(CatalogTitleDecoderError::InvalidCatalogTitle);
    }
    let appended: Vec<_> = catalog_characters
        .into_iter()
        .filter(|character| retained.insert(*character))
        .collect();
    let appended_catalog_character_count = appended.len();
    characters.extend(appended.into_iter().filter(|character| *character != ' '));
    characters.push(' ');
    let output_classes = characters
        .len()
        .checked_add(1)
        .ok_or(CatalogTitleDecoderError::InvalidCatalogTitle)?;
    Ok(TitleModelExportRequirements {
        schema: "scorepeek-title-model-export-requirements-v1",
        baseline_dictionary_sha256: TITLE_DICTIONARY_SHA256,
        dictionary_contract_id: "scorepeek-title-unicode-scalar-dictionary-v1",
        output_tensor_contract_id: "scorepeek-title-ctc-f32-logits-btc-v1",
        ctc_blank_token: 0,
        output_timesteps: OUTPUT_TIMESTEPS.max(required_timesteps),
        output_classes,
        baseline_character_count,
        appended_catalog_character_count,
        non_search_variant_count,
        covered_variant_count: non_search_variant_count,
        coverage_complete: true,
        non_blank_tokens: characters
            .into_iter()
            .map(|character| character.to_string())
            .collect(),
    })
}

fn audit_catalog_title_dictionary_with_indexes(
    catalog: &Catalog,
    indexes: &BTreeMap<char, u32>,
) -> CatalogTitleDictionaryAudit {
    let kinds = [
        DisplayVariantKind::InGameDisplay,
        DisplayVariantKind::OfficialDisplay,
        DisplayVariantKind::EamusementCsv,
        DisplayVariantKind::AlternateDisplay,
    ];
    let mut counts = BTreeMap::<DisplayVariantKind, VariantCoverageCounts>::new();
    let mut songs_without_non_search_variant = 0;
    for song in catalog.songs().values() {
        let mut has_non_search_variant = false;
        for variant in song
            .title_variants()
            .iter()
            .filter(|variant| variant.kind != DisplayVariantKind::SearchTerm)
        {
            has_non_search_variant = true;
            let coverage = counts.entry(variant.kind).or_default();
            coverage.variants += 1;
            let encodability = title_encodability(&variant.value, indexes);
            if encodability.unsupported_characters {
                coverage.unsupported_characters += 1;
            }
            if encodability.ctc_timestep_excess {
                coverage.ctc_timestep_excess += 1;
            }
            if !encodability.unsupported_characters && !encodability.ctc_timestep_excess {
                coverage.encodable += 1;
            }
        }
        if !has_non_search_variant {
            songs_without_non_search_variant += 1;
        }
    }
    let by_variant_kind: Vec<_> = kinds
        .into_iter()
        .map(|kind| {
            let coverage = counts.remove(&kind).unwrap_or_default();
            TitleDictionaryVariantKindAudit {
                kind,
                variant_count: coverage.variants,
                encodable_variant_count: coverage.encodable,
                rejected_variant_count: coverage.variants - coverage.encodable,
                unsupported_character_variant_count: coverage.unsupported_characters,
                ctc_timestep_excess_variant_count: coverage.ctc_timestep_excess,
            }
        })
        .collect();
    let non_search_variant_count = by_variant_kind
        .iter()
        .map(|coverage| coverage.variant_count)
        .sum();
    let encodable_variant_count = by_variant_kind
        .iter()
        .map(|coverage| coverage.encodable_variant_count)
        .sum();
    let rejected_variant_count = non_search_variant_count - encodable_variant_count;
    CatalogTitleDictionaryAudit {
        schema: "scorepeek-title-dictionary-coverage-audit-v1",
        dictionary_sha256: TITLE_DICTIONARY_SHA256,
        maximum_ctc_timesteps: MAX_TITLE_TOKENS,
        song_count: catalog.songs().len(),
        non_search_variant_count,
        encodable_variant_count,
        rejected_variant_count,
        songs_without_non_search_variant,
        coverage_complete: rejected_variant_count == 0 && songs_without_non_search_variant == 0,
        by_variant_kind,
    }
}

fn title_encodability(title: &str, indexes: &BTreeMap<char, u32>) -> TitleEncodability {
    let unsupported_characters = title
        .chars()
        .any(|character| !indexes.contains_key(&character));
    let characters: Vec<_> = title.chars().collect();
    let ctc_timesteps = characters.len()
        + characters
            .windows(2)
            .filter(|pair| pair[0] == pair[1])
            .count();
    TitleEncodability {
        unsupported_characters,
        ctc_timestep_excess: ctc_timesteps > MAX_TITLE_TOKENS,
    }
}

fn decide_ranked(
    ranked: &[(ScorepeekSongId, f64)],
    thresholds: DiagnosticTitleThresholds,
    catalog_coverage_complete: bool,
) -> CatalogTitleDecision {
    if !catalog_coverage_complete {
        return CatalogTitleDecision::Unknown {
            reason: CatalogTitleUnknownReason::CatalogCoverageIncomplete,
        };
    }
    let (top_id, top_score) = ranked[0];
    let runner_up = ranked.get(1).map_or(f64::NEG_INFINITY, |(_, score)| *score);
    if runner_up.total_cmp(&top_score).is_eq() {
        return CatalogTitleDecision::Unknown {
            reason: CatalogTitleUnknownReason::AmbiguousTopCandidate,
        };
    }
    if top_score < thresholds.minimum_log_probability {
        return CatalogTitleDecision::Unknown {
            reason: CatalogTitleUnknownReason::InsufficientAbsoluteEvidence,
        };
    }
    let margin = top_score - runner_up;
    if !margin.is_finite() || margin < thresholds.minimum_runner_up_margin {
        return CatalogTitleDecision::Unknown {
            reason: CatalogTitleUnknownReason::InsufficientRunnerUpMargin,
        };
    }
    CatalogTitleDecision::Unique {
        song_id: top_id,
        log_probability: top_score,
        runner_up_margin: margin,
    }
}

fn validate_probabilities(probabilities: &[f32]) -> Result<(), CatalogTitleDecoderError> {
    if probabilities.len() != OUTPUT_TIMESTEPS * OUTPUT_CLASSES {
        return Err(CatalogTitleDecoderError::InvalidProbabilities);
    }
    for row in probabilities.chunks_exact(OUTPUT_CLASSES) {
        let sum: f64 = row.iter().map(|value| f64::from(*value)).sum();
        if row.iter().any(|value| !value.is_finite() || *value <= 0.0) || (sum - 1.0).abs() > 2e-5 {
            return Err(CatalogTitleDecoderError::InvalidProbabilities);
        }
    }
    Ok(())
}

fn load_dictionary(path: &Path) -> Result<Vec<String>, CatalogTitleDecoderError> {
    load_dictionary_contract(path, TITLE_DICTIONARY_SHA256, OUTPUT_CLASSES)
}

pub(super) fn load_dictionary_contract(
    path: &Path,
    expected_sha256: &str,
    output_classes: usize,
) -> Result<Vec<String>, CatalogTitleDecoderError> {
    let metadata = path.metadata()?;
    if !metadata.is_file() || metadata.len() == 0 || metadata.len() > MAX_INFERENCE_YML_BYTES {
        return Err(CatalogTitleDecoderError::InvalidDictionary);
    }
    let capacity =
        usize::try_from(metadata.len()).map_err(|_| CatalogTitleDecoderError::InvalidDictionary)?;
    let mut bytes = Vec::with_capacity(capacity);
    File::open(path)?
        .take(MAX_INFERENCE_YML_BYTES + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 != metadata.len() || super::encode_sha256(&bytes) != expected_sha256 {
        return Err(CatalogTitleDecoderError::InvalidDictionary);
    }
    let text =
        std::str::from_utf8(&bytes).map_err(|_| CatalogTitleDecoderError::InvalidDictionary)?;
    let marker = "  character_dict:\n";
    let (_, body) = text
        .split_once(marker)
        .ok_or(CatalogTitleDecoderError::InvalidDictionary)?;
    let mut dictionary = Vec::with_capacity(output_classes);
    dictionary.push("blank".to_owned());
    for line in body.lines() {
        let Some(value) = line.strip_prefix("  - ") else {
            return Err(CatalogTitleDecoderError::InvalidDictionary);
        };
        dictionary.push(parse_yaml_scalar(value)?);
    }
    dictionary.push(" ".to_owned());
    if dictionary.len() != output_classes {
        return Err(CatalogTitleDecoderError::InvalidDictionary);
    }
    Ok(dictionary)
}

fn parse_yaml_scalar(value: &str) -> Result<String, CatalogTitleDecoderError> {
    let decoded = if let Some(inner) = value.strip_prefix('\'').and_then(|v| v.strip_suffix('\'')) {
        inner.replace("''", "'")
    } else {
        value.to_owned()
    };
    if decoded.is_empty() || decoded.chars().any(char::is_control) {
        return Err(CatalogTitleDecoderError::InvalidDictionary);
    }
    Ok(decoded)
}

fn dictionary_indexes(
    dictionary: &[String],
) -> Result<BTreeMap<char, u32>, CatalogTitleDecoderError> {
    let mut indexes = BTreeMap::new();
    let mut duplicates = BTreeSet::new();
    for (index, token) in dictionary.iter().enumerate().skip(1) {
        let mut chars = token.chars();
        let Some(character) = chars.next() else {
            return Err(CatalogTitleDecoderError::InvalidDictionary);
        };
        if chars.next().is_some() {
            continue;
        }
        let index =
            u32::try_from(index).map_err(|_| CatalogTitleDecoderError::InvalidDictionary)?;
        if indexes.insert(character, index).is_some() {
            duplicates.insert(character);
        }
    }
    for duplicate in duplicates {
        indexes.remove(&duplicate);
    }
    Ok(indexes)
}

fn tokenize(title: &str, indexes: &BTreeMap<char, u32>) -> Option<Vec<u32>> {
    if title.is_empty() || title.chars().any(char::is_control) {
        return None;
    }
    let tokens: Vec<_> = title
        .chars()
        .map(|character| indexes.get(&character).copied())
        .collect::<Option<_>>()?;
    let required_timesteps =
        tokens.len() + tokens.windows(2).filter(|pair| pair[0] == pair[1]).count();
    (required_timesteps <= MAX_TITLE_TOKENS).then_some(tokens)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn brute_force_ctc(probabilities: &[f32], classes: usize, expected: &[u32]) -> f64 {
        let timesteps = probabilities.len() / classes;
        let path_count = classes.pow(u32::try_from(timesteps).unwrap());
        let mut total = 0.0_f64;
        for mut encoded in 0..path_count {
            let mut path = vec![0_u32; timesteps];
            let mut probability = 1.0_f64;
            for timestep in (0..timesteps).rev() {
                let token = encoded % classes;
                encoded /= classes;
                path[timestep] = u32::try_from(token).unwrap();
                probability *= f64::from(probabilities[timestep * classes + token]);
            }
            let mut collapsed = Vec::new();
            let mut previous = None;
            for token in path {
                if token != 0 && previous != Some(token) {
                    collapsed.push(token);
                }
                previous = Some(token);
            }
            if collapsed == expected {
                total += probability;
            }
        }
        total.ln()
    }

    #[test]
    fn trie_scores_match_exhaustive_ctc_for_shared_prefixes_and_repeats() {
        let probabilities = [
            0.15, 0.70, 0.15, // timestep 0
            0.40, 0.35, 0.25, // timestep 1
            0.20, 0.30, 0.50, // timestep 2
            0.55, 0.25, 0.20, // timestep 3
        ];
        let sequences: [&[u32]; 3] = [&[1], &[1, 2], &[1, 1]];
        let mut trie = CtcSequenceTrie::default();
        for (index, sequence) in sequences.iter().enumerate() {
            assert!(trie.insert(sequence, index));
        }
        let scores = trie.score(&probabilities, 3).unwrap();
        for ((expected_index, sequence), (index, score)) in
            sequences.into_iter().enumerate().zip(scores.values)
        {
            let expected = brute_force_ctc(&probabilities, 3, sequence);
            assert_eq!(*index, expected_index);
            assert!((score - expected).abs() < 1e-12);
        }
    }

    #[test]
    fn tokenization_is_exact_and_accounts_for_ctc_repeat_blanks() {
        let indexes = BTreeMap::from([('A', 1), ('B', 2), (' ', 3)]);
        assert_eq!(tokenize("A B", &indexes), Some(vec![1, 3, 2]));
        assert_eq!(tokenize("AB!", &indexes), None);
        assert_eq!(tokenize("A\nB", &indexes), None);
        assert!(tokenize(&"A".repeat(21), &indexes).is_none());
        assert_eq!(tokenize(&"AB".repeat(20), &indexes).unwrap().len(), 40);
    }

    #[test]
    fn coverage_reports_unsupported_characters_and_ctc_length_independently() {
        let indexes = BTreeMap::from([('A', 1), ('B', 2)]);
        assert_eq!(
            title_encodability("AB", &indexes),
            TitleEncodability {
                unsupported_characters: false,
                ctc_timestep_excess: false,
            }
        );
        assert_eq!(
            title_encodability("A!", &indexes),
            TitleEncodability {
                unsupported_characters: true,
                ctc_timestep_excess: false,
            }
        );
        assert_eq!(
            title_encodability(&"AA".repeat(14), &indexes),
            TitleEncodability {
                unsupported_characters: false,
                ctc_timestep_excess: true,
            }
        );
    }

    #[test]
    fn duplicate_and_multiscalar_dictionary_entries_are_not_encodable() {
        let dictionary = vec![
            "blank".to_owned(),
            "A".to_owned(),
            "B".to_owned(),
            "A".to_owned(),
            "XY".to_owned(),
            " ".to_owned(),
        ];
        let indexes = dictionary_indexes(&dictionary).unwrap();
        assert_eq!(indexes, BTreeMap::from([('B', 2), (' ', 5)]));
    }

    #[test]
    fn model_export_requirements_retain_baseline_and_cover_every_variant() {
        let baseline = vec![
            "blank".to_owned(),
            "A".to_owned(),
            "XY".to_owned(),
            "A".to_owned(),
            " ".to_owned(),
        ];
        let requirements = build_title_model_export_requirements(
            &baseline,
            ["A A", "Ω", &"ZZ".repeat(30)].into_iter(),
        )
        .unwrap();
        assert_eq!(
            requirements.non_blank_tokens,
            ["A", "X", "Y", "Z", "Ω", " "]
        );
        assert_eq!(requirements.baseline_character_count, 4);
        assert_eq!(requirements.appended_catalog_character_count, 2);
        assert_eq!(requirements.non_search_variant_count, 3);
        assert_eq!(requirements.output_timesteps, 119);
        assert_eq!(requirements.output_classes, 7);
        assert_eq!(
            requirements.output_tensor_contract_id,
            "scorepeek-title-ctc-f32-logits-btc-v1"
        );
    }

    #[test]
    fn model_export_requirements_reject_invalid_or_missing_catalog_variants() {
        let baseline = vec!["blank".to_owned(), "A".to_owned()];
        assert!(matches!(
            build_title_model_export_requirements(&baseline, std::iter::empty()),
            Err(CatalogTitleDecoderError::InvalidCatalogTitle)
        ));
        assert!(matches!(
            build_title_model_export_requirements(&baseline, ["A\nB"].into_iter()),
            Err(CatalogTitleDecoderError::InvalidCatalogTitle)
        ));
    }

    #[test]
    fn yaml_scalar_parser_accepts_registered_forms_only() {
        assert_eq!(parse_yaml_scalar("A").unwrap(), "A");
        assert_eq!(parse_yaml_scalar("'A''B'").unwrap(), "A'B");
        assert!(parse_yaml_scalar("''").is_err());
        assert!(parse_yaml_scalar("'A\nB'").is_err());
    }

    #[test]
    fn decision_requires_unique_absolute_and_runner_up_evidence() {
        let first: ScorepeekSongId =
            serde_json::from_str("\"00000000-0000-0000-0000-000000000001\"").unwrap();
        let second: ScorepeekSongId =
            serde_json::from_str("\"00000000-0000-0000-0000-000000000002\"").unwrap();
        let thresholds = DiagnosticTitleThresholds {
            minimum_log_probability: -4.0,
            minimum_runner_up_margin: 2.0,
        };
        assert_eq!(
            decide_ranked(&[(first, -2.0), (second, -2.0)], thresholds, true),
            CatalogTitleDecision::Unknown {
                reason: CatalogTitleUnknownReason::AmbiguousTopCandidate,
            }
        );
        assert_eq!(
            decide_ranked(&[(first, -5.0), (second, -8.0)], thresholds, true),
            CatalogTitleDecision::Unknown {
                reason: CatalogTitleUnknownReason::InsufficientAbsoluteEvidence,
            }
        );
        assert_eq!(
            decide_ranked(&[(first, -2.0), (second, -3.0)], thresholds, true),
            CatalogTitleDecision::Unknown {
                reason: CatalogTitleUnknownReason::InsufficientRunnerUpMargin,
            }
        );
        assert_eq!(
            decide_ranked(&[(first, -2.0), (second, -5.0)], thresholds, true),
            CatalogTitleDecision::Unique {
                song_id: first,
                log_probability: -2.0,
                runner_up_margin: 3.0,
            }
        );
        assert_eq!(
            decide_ranked(&[(first, -2.0)], thresholds, true),
            CatalogTitleDecision::Unknown {
                reason: CatalogTitleUnknownReason::InsufficientRunnerUpMargin,
            }
        );
        assert_eq!(
            decide_ranked(&[(first, -2.0), (second, -5.0)], thresholds, false),
            CatalogTitleDecision::Unknown {
                reason: CatalogTitleUnknownReason::CatalogCoverageIncomplete,
            }
        );
    }
}
