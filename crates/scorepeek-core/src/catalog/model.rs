use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use unicode_normalization::UnicodeNormalization;
use uuid::Uuid;

use super::policy::{Completeness, LineageId, RevisionStrategy, SourceId, SourcePolicy};

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct SourceEvidence {
    pub source_id: SourceId,
    pub lineage_id: LineageId,
    pub revision_strategy: RevisionStrategy,
    pub revision: String,
    pub content_sha256: String,
    pub byte_size: usize,
    pub record_count: usize,
    pub parser_version: String,
    pub declared_scope: String,
    pub completeness: Completeness,
    pub field_authority: Vec<String>,
    pub freshness: String,
    pub rights_and_provenance: String,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
pub struct EvidenceId {
    pub source_id: SourceId,
    pub revision: String,
    pub content_sha256: String,
}

impl SourceEvidence {
    fn id(&self) -> EvidenceId {
        EvidenceId {
            source_id: self.source_id,
            revision: self.revision.clone(),
            content_sha256: self.content_sha256.clone(),
        }
    }
    #[must_use]
    pub const fn source_id(&self) -> SourceId {
        self.source_id
    }

    #[must_use]
    pub fn revision(&self) -> &str {
        &self.revision
    }

    #[must_use]
    pub fn content_sha256(&self) -> &str {
        &self.content_sha256
    }

    #[must_use]
    pub const fn record_count(&self) -> usize {
        self.record_count
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PlayType {
    Single,
    Double,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Difficulty {
    Beginner,
    Normal,
    Hyper,
    Another,
    Leggendaria,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
pub struct ChartKey {
    pub play_type: PlayType,
    pub difficulty: Difficulty,
}

#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
pub struct Chart {
    pub key: ChartKey,
    pub level: u8,
    pub notes: u32,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct ScorepeekSongId(Uuid);

impl ScorepeekSongId {
    #[must_use]
    pub fn from_tachi_id(tachi_id: &str) -> Self {
        let namespace = Uuid::new_v5(
            &Uuid::NAMESPACE_URL,
            b"https://github.com/atty303/scorepeek/song",
        );
        Self(Uuid::new_v5(&namespace, tachi_id.as_bytes()))
    }

    #[must_use]
    pub const fn as_uuid(self) -> Uuid {
        self.0
    }

    #[must_use]
    pub const fn from_uuid(value: Uuid) -> Self {
        Self(value)
    }
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
pub struct DisplayVariant {
    pub value: String,
    pub source_id: SourceId,
    pub kind: DisplayVariantKind,
    pub evidence_id: EvidenceId,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DisplayVariantKind {
    InGameDisplay,
    OfficialDisplay,
    EamusementCsv,
    AlternateDisplay,
    SearchTerm,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
pub struct ChartAssertion {
    pub source_chart_id: String,
    pub product_versions: BTreeSet<String>,
    pub primary: bool,
    pub evidence_id: EvidenceId,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum InfinitasStatus {
    ConfirmedPresent,
    Unknown,
    Conflicted,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct CatalogSong {
    pub song_id: ScorepeekSongId,
    pub tachi_source_id: String,
    pub title_variants: BTreeSet<DisplayVariant>,
    pub artist: String,
    pub version: String,
    pub charts: BTreeMap<ChartKey, Chart>,
    pub chart_assertions: BTreeMap<ChartKey, BTreeSet<ChartAssertion>>,
    pub infinitas_status: InfinitasStatus,
    pub source_bindings: BTreeMap<SourceId, BTreeSet<String>>,
    pub binding_evidence: BTreeMap<(SourceId, String), BTreeSet<EvidenceId>>,
    pub binding_attributes: BTreeMap<(SourceId, String, EvidenceId), BTreeMap<String, String>>,
    pub tachi_primary_infinitas: bool,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
pub struct Catalog {
    pub songs: BTreeMap<ScorepeekSongId, CatalogSong>,
    pub source_evidence: BTreeMap<EvidenceId, SourceEvidence>,
    pub latest_evidence: BTreeMap<SourceId, EvidenceId>,
    pub dqn_bindings: BTreeMap<ExactTitleArtist, DqnBinding>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct DqnBinding {
    pub song_id: ScorepeekSongId,
    pub evidence_packs: BTreeMap<EvidenceId, BTreeSet<Option<String>>>,
}

impl CatalogSong {
    #[must_use]
    pub const fn song_id(&self) -> ScorepeekSongId {
        self.song_id
    }

    #[must_use]
    pub fn title_variants(&self) -> &BTreeSet<DisplayVariant> {
        &self.title_variants
    }

    #[must_use]
    pub fn artist(&self) -> &str {
        &self.artist
    }

    #[must_use]
    pub fn charts(&self) -> &BTreeMap<ChartKey, Chart> {
        &self.charts
    }

    #[must_use]
    pub const fn infinitas_status(&self) -> InfinitasStatus {
        self.infinitas_status
    }
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
pub struct ExactTitleArtist {
    pub title: String,
    pub artist: String,
}

impl Catalog {
    #[must_use]
    pub fn songs(&self) -> &BTreeMap<ScorepeekSongId, CatalogSong> {
        &self.songs
    }

    #[must_use]
    pub fn source_evidence(&self) -> &BTreeMap<EvidenceId, SourceEvidence> {
        &self.source_evidence
    }

    /// Returns the versioned digest of every catalog value observable by `scorepeek run`.
    ///
    /// Source lineage, evidence, source-local bindings, quarantine state, and storage details are
    /// deliberately excluded. The encoded projection is ordered by the catalog's `BTreeMap` and
    /// `BTreeSet` keys, so rebuilding from the same accepted semantic records is stable.
    ///
    /// # Panics
    ///
    /// Panics only if serialization of the fixed, infallible semantic projection or writing to an
    /// in-memory `String` unexpectedly fails.
    #[must_use]
    pub fn semantic_digest(&self) -> String {
        #[derive(Serialize)]
        struct SemanticCatalog<'a> {
            schema: &'static str,
            songs: Vec<SemanticSong<'a>>,
        }

        #[derive(Serialize)]
        struct SemanticSong<'a> {
            song_id: ScorepeekSongId,
            titles: Vec<(DisplayVariantKind, &'a str)>,
            artist: &'a str,
            charts: Vec<&'a Chart>,
            infinitas_status: InfinitasStatus,
        }

        let songs = self
            .songs
            .values()
            .map(|song| SemanticSong {
                song_id: song.song_id,
                titles: song
                    .title_variants
                    .iter()
                    .map(|variant| (variant.kind, variant.value.as_str()))
                    .collect(),
                artist: &song.artist,
                charts: song.charts.values().collect(),
                infinitas_status: song.infinitas_status,
            })
            .collect();
        let encoded = serde_json::to_vec(&SemanticCatalog {
            schema: "scorepeek-catalog-runtime-semantics-v1",
            songs,
        })
        .expect("the semantic catalog projection is serializable");
        let digest = Sha256::digest(encoded);
        let mut value = String::with_capacity(digest.len() * 2);
        for byte in digest {
            use std::fmt::Write as _;
            write!(value, "{byte:02x}").expect("writing to a String cannot fail");
        }
        value
    }

    /// Validates the complete runtime catalog invariants.
    ///
    /// # Errors
    ///
    /// Returns a description when any song, chart, source policy, identity,
    /// or cross-reference invariant is invalid.
    pub fn validate(&self) -> Result<(), String> {
        validate_source_evidence(&self.source_evidence, &self.latest_evidence)?;
        validate_songs(&self.songs, &self.source_evidence)?;
        validate_dqn_bindings(&self.songs, &self.dqn_bindings, &self.source_evidence)
    }
}

fn validate_source_evidence(
    all_evidence: &BTreeMap<EvidenceId, SourceEvidence>,
    latest_evidence: &BTreeMap<SourceId, EvidenceId>,
) -> Result<(), String> {
    for (evidence_id, evidence) in all_evidence {
        let source_id = evidence.source_id;
        if evidence.id() != *evidence_id || evidence.record_count == 0 {
            return Err(format!("invalid source evidence for {source_id:?}"));
        }
        validate_evidence_policy(evidence)?;
        if !is_lower_hex(&evidence.content_sha256, 64) {
            return Err(format!("invalid content digest for {source_id:?}"));
        }
        let revision_length = match evidence.revision_strategy {
            RevisionStrategy::GitCommit => 40,
            RevisionStrategy::ContentSha256 => 64,
        };
        if !is_lower_hex(&evidence.revision, revision_length) {
            return Err(format!("invalid revision for {source_id:?}"));
        }
        if evidence.revision_strategy == RevisionStrategy::ContentSha256
            && evidence.revision != evidence.content_sha256
        {
            return Err(format!("content revision mismatch for {source_id:?}"));
        }
    }
    for (source_id, evidence_id) in latest_evidence {
        if evidence_id.source_id != *source_id || !all_evidence.contains_key(evidence_id) {
            return Err(format!("invalid latest evidence for {source_id:?}"));
        }
    }
    Ok(())
}

fn validate_evidence_policy(evidence: &SourceEvidence) -> Result<(), String> {
    let expected = SourcePolicy::for_id(evidence.source_id);
    if evidence.lineage_id != expected.lineage_id
        || evidence.revision_strategy != expected.revision_strategy
        || evidence.parser_version != expected.parser_version
        || evidence.declared_scope != expected.declared_scope
        || evidence.completeness != expected.completeness
        || evidence.field_authority
            != expected
                .field_authority
                .iter()
                .map(|field| (*field).to_owned())
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect::<Vec<_>>()
        || evidence.freshness != expected.freshness
        || evidence.rights_and_provenance != expected.rights_and_provenance
    {
        return Err(format!(
            "source policy mismatch for {:?}",
            evidence.source_id
        ));
    }
    Ok(())
}

fn validate_songs(
    songs: &BTreeMap<ScorepeekSongId, CatalogSong>,
    all_evidence: &BTreeMap<EvidenceId, SourceEvidence>,
) -> Result<(), String> {
    let mut global_bindings = BTreeSet::new();
    for (song_id, song) in songs {
        validate_song_identity(*song_id, song)?;
        validate_song_variants(*song_id, song, all_evidence)?;
        validate_song_charts(*song_id, song, all_evidence)?;
        for (source_id, bindings) in &song.source_bindings {
            if bindings.is_empty() {
                return Err(format!("song {song_id:?} has invalid source binding"));
            }
            for binding in bindings {
                validate_catalog_text("source_binding", binding)?;
                if !global_bindings.insert((*source_id, binding.clone())) {
                    return Err(format!("duplicate global source binding {binding:?}"));
                }
                let key = (*source_id, binding.clone());
                let Some(evidence_ids) = song.binding_evidence.get(&key) else {
                    return Err(format!("source binding {binding:?} lacks provenance"));
                };
                if evidence_ids.is_empty()
                    || evidence_ids
                        .iter()
                        .any(|id| id.source_id != *source_id || !all_evidence.contains_key(id))
                {
                    return Err(format!("source binding {binding:?} has invalid provenance"));
                }
            }
        }
        if song.binding_evidence.keys().any(|(source_id, source_key)| {
            !song
                .source_bindings
                .get(source_id)
                .is_some_and(|bindings| bindings.contains(source_key))
        }) {
            return Err(format!("song {song_id:?} has orphan binding provenance"));
        }
        for ((source_id, source_key, evidence_id), attributes) in &song.binding_attributes {
            if evidence_id.source_id != *source_id
                || attributes.is_empty()
                || !song
                    .binding_evidence
                    .get(&(*source_id, source_key.clone()))
                    .is_some_and(|ids| ids.contains(evidence_id))
            {
                return Err(format!("song {song_id:?} has invalid binding attributes"));
            }
            for (key, value) in attributes {
                validate_catalog_text("binding_attribute_key", key)?;
                validate_catalog_text("binding_attribute_value", value)?;
            }
        }
        validate_required_binding_attributes(*song_id, song)?;
    }
    Ok(())
}

fn validate_song_identity(song_id: ScorepeekSongId, song: &CatalogSong) -> Result<(), String> {
    if song.song_id != song_id || ScorepeekSongId::from_tachi_id(&song.tachi_source_id) != song_id {
        return Err(format!("invalid Tachi-derived song ID {song_id:?}"));
    }
    validate_catalog_text("tachi_source_id", &song.tachi_source_id)?;
    validate_catalog_text("artist", &song.artist)?;
    validate_catalog_text("version", &song.version)?;
    let tachi_bindings = song.source_bindings.get(&SourceId::Tachi);
    if !tachi_bindings
        .is_some_and(|bindings| bindings.len() == 1 && bindings.contains(&song.tachi_source_id))
    {
        return Err(format!("song {song_id:?} has invalid Tachi binding"));
    }
    Ok(())
}

fn validate_song_variants(
    song_id: ScorepeekSongId,
    song: &CatalogSong,
    all_evidence: &BTreeMap<EvidenceId, SourceEvidence>,
) -> Result<(), String> {
    if song.title_variants.is_empty() {
        return Err(format!("song {song_id:?} has no title variants"));
    }
    for variant in &song.title_variants {
        validate_catalog_text("title_variant", &variant.value)?;
        if variant.evidence_id.source_id != variant.source_id
            || !all_evidence.contains_key(&variant.evidence_id)
            || song
                .source_bindings
                .get(&variant.source_id)
                .is_none_or(BTreeSet::is_empty)
        {
            return Err(format!(
                "song {song_id:?} variant lacks source evidence {:?}",
                variant.source_id
            ));
        }
    }
    Ok(())
}

fn validate_song_charts(
    song_id: ScorepeekSongId,
    song: &CatalogSong,
    all_evidence: &BTreeMap<EvidenceId, SourceEvidence>,
) -> Result<(), String> {
    for (key, chart) in &song.charts {
        if chart.key != *key || !(1..=12).contains(&chart.level) || chart.notes == 0 {
            return Err(format!("song {song_id:?} has invalid chart {key:?}"));
        }
        let Some(assertions) = song.chart_assertions.get(key) else {
            return Err(format!("song {song_id:?} chart {key:?} lacks provenance"));
        };
        if assertions.is_empty()
            || assertions.iter().any(|assertion| {
                assertion.source_chart_id.is_empty()
                    || assertion.product_versions.is_empty()
                    || !all_evidence.contains_key(&assertion.evidence_id)
                    || song
                        .source_bindings
                        .get(&assertion.evidence_id.source_id)
                        .is_none_or(BTreeSet::is_empty)
            })
        {
            return Err(format!(
                "song {song_id:?} chart {key:?} has invalid provenance"
            ));
        }
    }
    if song
        .chart_assertions
        .keys()
        .any(|key| !song.charts.contains_key(key))
    {
        return Err(format!(
            "song {song_id:?} has provenance for an absent chart"
        ));
    }
    Ok(())
}

fn validate_dqn_bindings(
    songs: &BTreeMap<ScorepeekSongId, CatalogSong>,
    bindings: &BTreeMap<ExactTitleArtist, DqnBinding>,
    all_evidence: &BTreeMap<EvidenceId, SourceEvidence>,
) -> Result<(), String> {
    for (tuple, binding) in bindings {
        let song_id = binding.song_id;
        let Some(song) = songs.get(&song_id) else {
            return Err(format!("dqn binding references absent song {song_id:?}"));
        };
        if tuple.title != nfc(&tuple.title)
            || tuple.artist != nfc(&tuple.artist)
            || nfc(&song.artist) != tuple.artist
            || !song
                .title_variants
                .iter()
                .any(|variant| identity_variant(variant) && nfc(&variant.value) == tuple.title)
        {
            return Err(format!("invalid dqn binding for song {song_id:?}"));
        }
        if binding.evidence_packs.is_empty()
            || binding.evidence_packs.iter().any(|(id, packs)| {
                id.source_id != SourceId::DqnIidxapi
                    || !all_evidence.contains_key(id)
                    || packs.is_empty()
                    || packs.iter().any(|pack| {
                        pack.as_ref()
                            .is_some_and(|pack| validate_catalog_text("dqn_pack", pack).is_err())
                    })
            })
        {
            return Err(format!("dqn binding lacks evidence for song {song_id:?}"));
        }
    }
    let dqn_song_ids: BTreeSet<_> = bindings.values().map(|binding| binding.song_id).collect();
    for song in songs.values() {
        let tachi_primary = tachi_primary_from_attributes(song)?;
        if song.tachi_primary_infinitas != tachi_primary {
            return Err(format!(
                "song {:?} has unproven Tachi availability",
                song.song_id
            ));
        }
        let expected = if tachi_primary || dqn_song_ids.contains(&song.song_id) {
            InfinitasStatus::ConfirmedPresent
        } else {
            InfinitasStatus::Unknown
        };
        if song.infinitas_status != expected {
            return Err(format!(
                "song {:?} has inconsistent availability",
                song.song_id
            ));
        }
    }
    Ok(())
}

fn validate_required_binding_attributes(
    song_id: ScorepeekSongId,
    song: &CatalogSong,
) -> Result<(), String> {
    for ((source_id, source_key), evidence_ids) in &song.binding_evidence {
        for evidence_id in evidence_ids {
            let attributes = song
                .binding_attributes
                .get(&(*source_id, source_key.clone(), evidence_id.clone()))
                .ok_or_else(|| format!("song {song_id:?} binding lacks typed attributes"))?;
            match source_id {
                SourceId::Tachi => {
                    if attributes.len() != 1
                        || attributes
                            .get("primary_infinitas")
                            .and_then(|value| value.parse::<bool>().ok())
                            .is_none()
                    {
                        return Err(format!("song {song_id:?} has invalid Tachi attributes"));
                    }
                }
                SourceId::Textage => validate_textage_attributes(song_id, attributes)?,
                SourceId::DqnIidxapi => {
                    return Err(format!("song {song_id:?} has invalid dqn source binding"));
                }
            }
        }
    }
    Ok(())
}

fn validate_textage_attributes(
    song_id: ScorepeekSongId,
    attributes: &BTreeMap<String, String>,
) -> Result<(), String> {
    let flag = attributes
        .get("infinitas_flag")
        .and_then(|value| value.parse::<bool>().ok());
    let minimum = attributes
        .get("bpm_min")
        .and_then(|value| value.parse::<u16>().ok());
    let maximum = attributes
        .get("bpm_max")
        .and_then(|value| value.parse::<u16>().ok());
    if attributes.len() != 3
        || flag.is_none()
        || minimum.is_none()
        || maximum.is_none()
        || (minimum == Some(0)) != (maximum == Some(0))
        || minimum > maximum
    {
        return Err(format!("song {song_id:?} has invalid Textage attributes"));
    }
    Ok(())
}

fn tachi_primary_from_attributes(song: &CatalogSong) -> Result<bool, String> {
    let evidence_ids = song
        .binding_evidence
        .get(&(SourceId::Tachi, song.tachi_source_id.clone()))
        .ok_or_else(|| format!("song {:?} lacks Tachi evidence", song.song_id))?;
    evidence_ids.iter().try_fold(false, |primary, evidence_id| {
        let value = song
            .binding_attributes
            .get(&(
                SourceId::Tachi,
                song.tachi_source_id.clone(),
                evidence_id.clone(),
            ))
            .and_then(|attributes| attributes.get("primary_infinitas"))
            .and_then(|value| value.parse::<bool>().ok())
            .ok_or_else(|| format!("song {:?} lacks Tachi availability evidence", song.song_id))?;
        Ok(primary || value)
    })
}

fn is_lower_hex(value: &str, length: usize) -> bool {
    value.len() == length
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

fn validate_catalog_text(field: &str, value: &str) -> Result<(), String> {
    if value.is_empty() || value.len() > 512 || value.chars().any(char::is_control) {
        return Err(format!("invalid {field}"));
    }
    Ok(())
}

fn identity_variant(variant: &DisplayVariant) -> bool {
    !matches!(variant.kind, DisplayVariantKind::SearchTerm)
}

fn nfc(value: &str) -> String {
    value.nfc().collect()
}
