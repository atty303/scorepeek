use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::io::Read as _;
use std::path::Path;

use serde::Serialize;

use crate::catalog::{Catalog, DisplayVariantKind, ScorepeekSongId};

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
    InvalidProbabilities,
    InvalidThresholds,
}

impl std::fmt::Display for CatalogTitleDecoderError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "catalog title decoder I/O failed: {error}"),
            Self::InvalidDictionary => {
                formatter.write_str("catalog title decoder dictionary is invalid")
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

#[derive(Default)]
struct TrieNode {
    token: u32,
    parent: usize,
    children: BTreeMap<u32, usize>,
    songs: BTreeSet<ScorepeekSongId>,
}

/// Scores every exactly encodable non-search catalog title through one CTC prefix trie.
///
/// This is an offline diagnostic boundary. Thresholds are explicit because the current profile
/// has no calibrated acceptance policy. A title that cannot be represented by the registered
/// model dictionary is not approximated or normalized.
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
    let mut trie = vec![TrieNode::default()];
    let mut catalog_coverage_complete = true;
    for (song_id, song) in catalog.songs() {
        let mut has_non_search_variant = false;
        for variant in song
            .title_variants()
            .iter()
            .filter(|variant| variant.kind != DisplayVariantKind::SearchTerm)
        {
            has_non_search_variant = true;
            let Some(tokens) = tokenize(&variant.value, &indexes) else {
                catalog_coverage_complete = false;
                continue;
            };
            let mut node = 0;
            for token in tokens {
                node = if let Some(child) = trie[node].children.get(&token) {
                    *child
                } else {
                    let child = trie.len();
                    trie.push(TrieNode {
                        token,
                        parent: node,
                        ..TrieNode::default()
                    });
                    trie[node].children.insert(token, child);
                    child
                };
            }
            trie[node].songs.insert(*song_id);
        }
        catalog_coverage_complete &= has_non_search_variant;
    }
    if trie.iter().all(|node| node.songs.is_empty()) {
        return Ok(CatalogTitleDecision::Unknown {
            reason: CatalogTitleUnknownReason::NoEncodableCandidate,
        });
    }

    let scores = score_trie(probabilities, &trie, OUTPUT_CLASSES);
    let mut songs = BTreeMap::<ScorepeekSongId, f64>::new();
    for (node, score) in trie.iter().zip(scores) {
        for song_id in &node.songs {
            songs
                .entry(*song_id)
                .and_modify(|existing| *existing = existing.max(score))
                .or_insert(score);
        }
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
    let metadata = path.symlink_metadata()?;
    if !metadata.is_file() || metadata.len() == 0 || metadata.len() > MAX_INFERENCE_YML_BYTES {
        return Err(CatalogTitleDecoderError::InvalidDictionary);
    }
    let capacity =
        usize::try_from(metadata.len()).map_err(|_| CatalogTitleDecoderError::InvalidDictionary)?;
    let mut bytes = Vec::with_capacity(capacity);
    File::open(path)?
        .take(MAX_INFERENCE_YML_BYTES + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 != metadata.len()
        || super::encode_sha256(&bytes) != TITLE_DICTIONARY_SHA256
    {
        return Err(CatalogTitleDecoderError::InvalidDictionary);
    }
    let text =
        std::str::from_utf8(&bytes).map_err(|_| CatalogTitleDecoderError::InvalidDictionary)?;
    let marker = "  character_dict:\n";
    let (_, body) = text
        .split_once(marker)
        .ok_or(CatalogTitleDecoderError::InvalidDictionary)?;
    let mut dictionary = Vec::with_capacity(OUTPUT_CLASSES);
    dictionary.push("blank".to_owned());
    for line in body.lines() {
        let Some(value) = line.strip_prefix("  - ") else {
            return Err(CatalogTitleDecoderError::InvalidDictionary);
        };
        dictionary.push(parse_yaml_scalar(value)?);
    }
    dictionary.push(" ".to_owned());
    if dictionary.len() != OUTPUT_CLASSES {
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

fn score_trie(probabilities: &[f32], trie: &[TrieNode], classes: usize) -> Vec<f64> {
    let mut blank = vec![f64::NEG_INFINITY; trie.len()];
    let mut nonblank = vec![f64::NEG_INFINITY; trie.len()];
    blank[0] = 0.0;
    for row in probabilities.chunks_exact(classes) {
        let mut next_blank = vec![f64::NEG_INFINITY; trie.len()];
        let mut next_nonblank = vec![f64::NEG_INFINITY; trie.len()];
        let blank_probability = f64::from(row[0]).ln();
        next_blank[0] = blank[0] + blank_probability;
        for index in 1..trie.len() {
            next_blank[index] = logsumexp([blank[index], nonblank[index]]) + blank_probability;
            let node = &trie[index];
            let parent = &trie[node.parent];
            let mut sources = [f64::NEG_INFINITY; 3];
            sources[0] = nonblank[index];
            sources[1] = blank[node.parent];
            if node.token != parent.token || node.parent == 0 {
                sources[2] = nonblank[node.parent];
            }
            next_nonblank[index] = logsumexp(sources) + f64::from(row[node.token as usize]).ln();
        }
        blank = next_blank;
        nonblank = next_nonblank;
    }
    blank
        .into_iter()
        .zip(nonblank)
        .map(|(blank, nonblank)| logsumexp([blank, nonblank]))
        .collect()
}

fn logsumexp<const N: usize>(values: [f64; N]) -> f64 {
    let maximum = values.into_iter().fold(f64::NEG_INFINITY, f64::max);
    if maximum == f64::NEG_INFINITY {
        maximum
    } else {
        maximum
            + values
                .into_iter()
                .map(|value| (value - maximum).exp())
                .sum::<f64>()
                .ln()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build_trie(sequences: &[&[u32]]) -> (Vec<TrieNode>, Vec<usize>) {
        let mut trie = vec![TrieNode::default()];
        let mut terminals = Vec::new();
        for sequence in sequences {
            let mut node = 0;
            for &token in *sequence {
                node = if let Some(child) = trie[node].children.get(&token) {
                    *child
                } else {
                    let child = trie.len();
                    trie.push(TrieNode {
                        token,
                        parent: node,
                        ..TrieNode::default()
                    });
                    trie[node].children.insert(token, child);
                    child
                };
            }
            terminals.push(node);
        }
        (trie, terminals)
    }

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
        let (trie, terminals) = build_trie(&sequences);
        let scores = score_trie(&probabilities, &trie, 3);
        for (sequence, terminal) in sequences.into_iter().zip(terminals) {
            let expected = brute_force_ctc(&probabilities, 3, sequence);
            assert!((scores[terminal] - expected).abs() < 1e-12);
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
